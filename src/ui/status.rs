//! Compact runtime/session status rows shown below the composer.
use super::{
    task,
    text::{compact_number, truncate_middle},
    theme,
};
use crate::app::App;
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style},
    widgets::{Block, Paragraph},
};

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    frame.render_widget(Block::default().style(theme::surface()), area);
    let mut lines = vec![status_line(app, width)];
    if area.height > 1 {
        lines.push(status_aux_line(app, width));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

pub(super) fn status_line(app: &App, width: usize) -> Line<'static> {
    if !app.follow_tail && app.max_scroll > 0 {
        let remaining = app.max_scroll.saturating_sub(app.scroll_y);
        let candidates = [
            format!(" History · {remaining} lines below · Ctrl+End to return to latest "),
            format!(" {remaining} below · Ctrl+End latest "),
            " Ctrl+End latest ".to_owned(),
        ];
        let text = candidates
            .iter()
            .find(|text| Span::raw(text.as_str()).width() <= width)
            .unwrap_or(&candidates[2]);
        return Line::styled(
            task::fit(text, width),
            Style::default().fg(theme::warning()),
        );
    }

    let current_tokens = app.state.current_context_tokens.unwrap_or(0);
    let total_tokens = app
        .state
        .active_model_context_length
        .filter(|_| app.state.current_context_tokens.is_some());
    let reasoning = if app.state.active_reasoning_level.is_empty() {
        "auto"
    } else {
        app.state.active_reasoning_level.as_str()
    };
    let permission = app
        .state
        .sandbox_settings
        .as_ref()
        .map(|settings| settings.permission_mode())
        .unwrap_or("ask");
    let current = compact_number(current_tokens);
    let prefix = if width >= 4 { "  \u{25c8} " } else { "" };
    let available = width.saturating_sub(Span::raw(prefix).width());
    let model_full = if app.state.active_model.is_empty() {
        "no model".to_owned()
    } else {
        truncate_middle(&app.state.active_model, 30)
    };
    let model_short = if app.state.active_model.is_empty() {
        "no model".to_owned()
    } else {
        let short = app
            .state
            .active_model
            .rsplit_once('/')
            .map(|(_, model)| model)
            .unwrap_or(app.state.active_model.as_str());
        truncate_middle(short, 18)
    };

    if width >= 76
        && let Some(total) = total_tokens.filter(|total| *total > 0)
    {
        let ratio = current_tokens as f64 / total as f64;
        let pressure = if ratio >= 0.95 {
            theme::error()
        } else if ratio >= 0.80 {
            theme::warning()
        } else {
            theme::user()
        };
        let percent = format!("{:.0}%", ratio * 100.0);
        let meter = context_meter(current_tokens, total_tokens, 7);
        let line = Line::from(vec![
            Span::styled(
                prefix,
                Style::default()
                    .fg(theme::accent())
                    .bg(theme::surface_raised())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{model_short} "),
                Style::default()
                    .fg(theme::accent())
                    .bg(theme::surface_raised())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  │  ", Style::default().fg(theme::border_dim())),
            Span::styled("CTX ", Style::default().fg(theme::muted())),
            Span::styled(
                format!("{percent} "),
                Style::default().fg(pressure).add_modifier(Modifier::BOLD),
            ),
            Span::styled(meter, Style::default().fg(pressure)),
            Span::styled("  │  ", Style::default().fg(theme::border_dim())),
            Span::styled("THINK ", Style::default().fg(theme::muted())),
            Span::styled(
                reasoning.to_owned(),
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  │  ", Style::default().fg(theme::border_dim())),
            Span::styled("MODE ", Style::default().fg(theme::muted())),
            Span::styled(
                permission.to_owned(),
                Style::default()
                    .fg(theme::text_dim())
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        if line.width() <= width {
            return line;
        }
    }
    let model_tiny = truncate_middle(&model_short, 11);
    let reasoning_short = match reasoning {
        "medium" => "med",
        "auto" => "auto",
        "low" => "low",
        "high" => "high",
        other => other,
    };
    let permission_short = match permission {
        "unlimited" => "unlim",
        other => other,
    };
    let candidates = if let Some(total) = total_tokens.filter(|total| *total > 0) {
        let total_label = compact_number(total);
        let percent = format!("{:.0}%", current_tokens as f64 * 100.0 / total as f64);
        let meter = context_meter(current_tokens, total_tokens, 7);
        vec![
            format!(
                "{model_full}  ·  ctx {current}/{total_label} {percent} {meter}  ·  think {reasoning}  ·  {permission}"
            ),
            format!("{model_short}  ·  ctx {percent} {meter}  ·  {reasoning}  ·  {permission}"),
            format!("{model_short}  ·  {percent} ctx  ·  {permission_short}"),
            format!("{model_tiny}  ·  {percent} ctx"),
            format!("{percent} ctx"),
        ]
    } else {
        let model_label = if app.state.active_model.is_empty() {
            model_full.clone()
        } else {
            format!("model {model_full}")
        };
        vec![
            format!("{model_label}  ·  ctx unavailable  ·  think {reasoning}  ·  {permission}"),
            format!("{model_short}  ·  ctx —  ·  {reasoning}  ·  {permission}"),
            format!("{model_short}  ·  ctx —  ·  {reasoning_short}/{permission_short}"),
            format!("{model_tiny} · {permission_short}"),
            model_tiny.clone(),
        ]
    };
    let fallback = candidates.last().cloned().unwrap_or_default();
    let text = candidates
        .into_iter()
        .find(|candidate| Span::raw(candidate).width() <= available)
        .unwrap_or_else(|| task::fit(&fallback, available));

    Line::from(
        vec![Span::styled(
            prefix,
            Style::default()
                .fg(theme::accent())
                .bg(theme::surface_raised())
                .add_modifier(Modifier::BOLD),
        )]
        .into_iter()
        .chain(
            text.split_inclusive('·')
                .enumerate()
                .map(|(index, segment)| {
                    let fg = match index {
                        0 => theme::accent(),
                        1 if total_tokens.is_some_and(|total| {
                            total > 0 && current_tokens as f64 / total as f64 >= 0.95
                        }) =>
                        {
                            theme::error()
                        }
                        1 if total_tokens.is_some_and(|total| {
                            total > 0 && current_tokens as f64 / total as f64 >= 0.80
                        }) =>
                        {
                            theme::warning()
                        }
                        1 if total_tokens.is_some_and(|total| total > 0) => theme::user(),
                        1 => theme::muted(),
                        _ => theme::muted(),
                    };
                    let mut style = Style::default().fg(fg);
                    if index == 0 {
                        style = style
                            .bg(theme::surface_raised())
                            .add_modifier(Modifier::BOLD);
                    } else if index == 1 && total_tokens.is_some_and(|total| total > 0) {
                        style = style.bg(theme::code_background());
                    }
                    Span::styled(segment.to_owned(), style)
                }),
        )
        .collect::<Vec<_>>(),
    )
}

pub(super) fn status_aux_line(app: &App, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let prefix = if width >= 4 { "  ╰ " } else { "" };
    let available = width.saturating_sub(Span::raw(prefix).width());

    let (candidates, color) = if app.selection_start.is_some() || app.selection_end.is_some() {
        (
            vec![
                "selection · Ctrl+C copy · Esc clear".to_owned(),
                "selected · Ctrl+C copy · Esc clear".to_owned(),
                "Ctrl+C copy · Esc clear".to_owned(),
                "Esc clear".to_owned(),
            ],
            theme::user(),
        )
    } else {
        let usage = &app.state.token_usage;
        let has_usage = usage.input_tokens.is_some()
            || usage.output_tokens.is_some()
            || usage.model_calls.is_some();
        if has_usage {
            let input = compact_number(usage.input_tokens.unwrap_or(0));
            let output = compact_number(usage.output_tokens.unwrap_or(0));
            let calls = usage.model_calls.unwrap_or(0);
            let cache = usage
                .cache_measurement()
                .hit_rate
                .map(|rate| format!("{:.0}%", rate * 100.0))
                .unwrap_or_else(|| "—".to_owned());
            (
                vec![
                    format!("session  in {input} · out {output} · cache {cache} · calls {calls}"),
                    format!("in {input} · out {output} · cache {cache} · {calls} calls"),
                    format!("in {input} · out {output} · cache {cache}"),
                    format!("in {input} · out {output}"),
                    format!("in {input}"),
                ],
                theme::text_dim(),
            )
        } else {
            (
                vec![
                    "Ctrl+N new · Alt+M model · Alt+S sessions · ? help".to_owned(),
                    "Ctrl+N new · Alt+M model · ? help".to_owned(),
                    "? help · /status".to_owned(),
                ],
                theme::muted(),
            )
        }
    };

    let fallback = candidates.last().cloned().unwrap_or_default();
    let text = candidates
        .into_iter()
        .find(|candidate| Span::raw(candidate).width() <= available)
        .unwrap_or_else(|| task::fit(&fallback, available));
    Line::from(vec![
        Span::styled(prefix, Style::default().fg(theme::border_dim())),
        Span::styled(text, Style::default().fg(color)),
    ])
}

fn context_meter(current: u64, total: Option<u64>, cells: usize) -> String {
    let Some(total) = total.filter(|total| *total > 0) else {
        return "·".repeat(cells);
    };
    let filled =
        ((current.saturating_mul(cells as u64) + total / 2) / total).min(cells as u64) as usize;
    format!(
        "{}{}",
        "▰".repeat(filled),
        "▱".repeat(cells.saturating_sub(filled))
    )
}
