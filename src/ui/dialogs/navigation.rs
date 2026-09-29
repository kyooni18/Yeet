//! Model, Goal, session, and reasoning navigation dialogs.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelPickerRow {
    pub(super) id: String,
    pub(super) provider: String,
    pub(super) provider_heading: String,
    pub(super) model: String,
    pub(super) context_length: Option<u64>,
}

pub(super) fn model_picker_rows(app: &App) -> Vec<ModelPickerRow> {
    let mut previous_provider = String::new();
    app.filtered_models()
        .into_iter()
        .map(|id| {
            let (provider, model, context_length) = app
                .model_catalog_item(id)
                .map(|item| {
                    (
                        item.provider.clone(),
                        item.model.clone(),
                        item.context_length,
                    )
                })
                .unwrap_or_else(|| {
                    id.split_once('/')
                        .map(|(provider, model)| (provider.to_owned(), model.to_owned(), None))
                        .unwrap_or_else(|| ("other".to_owned(), id.to_owned(), None))
                });
            let provider_heading = if provider == previous_provider {
                String::new()
            } else {
                previous_provider = provider.clone();
                match provider.as_str() {
                    "codex-cli" => "Codex CLI".to_owned(),
                    "antigravity" => "Antigravity".to_owned(),
                    "gemini" => "Gemini".to_owned(),
                    "gemini-web" => "Gemini Web".to_owned(),
                    "anthropic" => "Claude (Anthropic)".to_owned(),
                    "claude" => "Claude Web".to_owned(),
                    _ => provider.clone(),
                }
            };
            ModelPickerRow {
                id: id.to_owned(),
                provider,
                provider_heading,
                model,
                context_length,
            }
        })
        .collect()
}

pub(crate) fn draw_models(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(76, 74, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.debate_model_picker {
        let role = match app.debate_field {
            1 => "Pro",
            2 => "Con",
            3 => "Jury",
            _ => "Model",
        };
        format!(" {role} model · ↑/↓ navigate · type filter · Enter select · Esc back ")
    } else if app.state.is_streaming {
        " Models · next response · ↑/↓ navigate · type filter · Enter select · Esc close "
            .to_owned()
    } else {
        " Models · ↑/↓ navigate · type filter · Enter select · Esc close ".to_owned()
    };
    let block = theme::modal_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(inner);
    let rows = model_picker_rows(app);
    let provider_count = rows
        .iter()
        .map(|row| row.provider.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let query = if app.popup_filter.is_empty() {
        "all".to_owned()
    } else {
        app.popup_filter.clone()
    };
    let refreshing = if app.state.is_loading_models {
        " · refreshing…"
    } else {
        ""
    };
    let navigation_hint = if chunks[0].width >= 60 {
        " · ↑/↓ PgUp/PgDn"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Search  ", Style::default().fg(theme::accent()).bold()),
            Span::styled(query, Style::default().fg(theme::text())),
            Span::styled(
                format!(
                    "  ·  {} models  ·  {provider_count} providers{refreshing}{navigation_hint}",
                    rows.len()
                ),
                Style::default().fg(theme::muted()),
            ),
        ])),
        chunks[0],
    );

    if app.state.is_loading_models && rows.is_empty() {
        frame.render_widget(
            Paragraph::new("Loading model catalog…").fg(theme::text_dim()),
            chunks[1],
        );
        return;
    }
    if rows.is_empty() {
        let (message, color) = app
            .state
            .error_message
            .as_deref()
            .map(|message| (message, theme::error()))
            .unwrap_or((
                "No matching models. Search by provider or model.",
                theme::muted(),
            ));
        frame.render_widget(Paragraph::new(message).fg(color), chunks[1]);
        return;
    }

    let provider_width = if chunks[1].width < 54 { 9 } else { 13 };
    let items = rows.iter().map(|row| {
        let marker = if row.id == app.state.active_model {
            "●"
        } else {
            " "
        };
        let context = row
            .context_length
            .map(|length| format!(" · {} ctx", compact_number(length)))
            .unwrap_or_default();
        let model_budget = (chunks[1].width as usize)
            .saturating_sub(provider_width + context.chars().count() + 6)
            .max(8);
        let provider = truncate_end(&row.provider_heading, provider_width);
        ListItem::new(Line::from(vec![
            Span::raw(format!("{marker} {provider:<provider_width$} ")),
            Span::styled(
                truncate_middle(&row.model, model_budget),
                Style::default().fg(theme::text()),
            ),
            Span::styled(context, Style::default().fg(theme::muted())),
        ]))
    });
    let list = List::new(items)
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

pub(crate) fn draw_goal(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(64, 42, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" Goal · ↑/↓ select · Enter apply · Esc close ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Length(4), Constraint::Min(2)]).split(inner);
    let status = if app.state.goal_mode { "ON" } else { "OFF" };
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(vec![
                Span::styled(
                    " Goal  ",
                    Style::default().fg(theme::accent()).bold(),
                ),
                Span::styled(status, Style::default().fg(theme::text()).bold()),
            ]),
            Line::from(""),
            Line::from("Runs bounded execution epochs until the strict success judge accepts concrete evidence."),
            Line::from("Transient provider errors back off and retry; interrupt or turn Goal off to stop."),
        ])),
        chunks[0],
    );
    let items = [
        (true, "ON", "Strict success judged execution"),
        (false, "OFF", "Normal completion behavior"),
    ]
    .into_iter()
    .map(|(enabled, label, description)| {
        let marker = if app.state.goal_mode == enabled {
            "●"
        } else {
            " "
        };
        ListItem::new(Line::from(vec![
            Span::raw(format!("{marker} {label:<3}  ")),
            Span::styled(description, Style::default().fg(theme::text())),
        ]))
    });
    let list = List::new(items)
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.popup_index.min(1)));
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

pub(crate) fn draw_sessions(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(82, 76, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" Sessions · Enter open · Ctrl+N new · Esc close ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let all_sessions = app.session_picker_items();
    let sessions = app.filtered_session_picker_items();
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(inner);
    let filter_label = if app.popup_filter.is_empty() {
        "type to filter".to_owned()
    } else {
        app.popup_filter.clone()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Search  ", Style::default().fg(theme::accent()).bold()),
            Span::styled(filter_label, Style::default().fg(theme::text())),
            Span::styled(
                format!(
                    "  ·  {}/{}  ·  ↑/↓ PgUp/PgDn",
                    sessions.len(),
                    all_sessions.len()
                ),
                Style::default().fg(theme::muted()),
            ),
        ])),
        rows[0],
    );

    if all_sessions.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("No saved sessions."),
                Line::styled(
                    "Press Ctrl+N to start a new session.",
                    Style::default().fg(theme::muted()),
                ),
            ]),
            rows[1],
        );
        return;
    }
    if sessions.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("No matching sessions."),
                Line::styled(
                    "Backspace to broaden the filter.",
                    Style::default().fg(theme::muted()),
                ),
            ]),
            rows[1],
        );
        return;
    }

    let row_width = rows[1].width.saturating_sub(2) as usize;
    // Keep titles on their own line instead of letting workspace/model columns
    // consume the label budget. Very short terminals retain compact rows.
    let show_details = rows[1].height >= 4;
    let items = sessions.iter().map(|item| {
        let current = app.state.current_session_id.as_deref() == Some(item.session.id.as_str());
        let marker = crate::ui::icons::session(current);
        let badge = if current && row_width >= 20 {
            " · current"
        } else {
            ""
        };
        let title_budget = row_width.saturating_sub(2 + cell_width(badge));
        let title = Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(theme::accent())),
            Span::styled(
                truncate_end(&item.session.display_title(), title_budget),
                Style::default().fg(theme::text()).bold(),
            ),
            Span::styled(badge, Style::default().fg(theme::accent())),
        ]);
        if !show_details {
            return ListItem::new(title);
        }
        let workspace_style = if item.workspace_current {
            Style::default().fg(theme::accent())
        } else {
            Style::default().fg(theme::muted())
        };
        let detail_budget = row_width.saturating_sub(2);
        let workspace_budget = (detail_budget / 3).min(20);
        let workspace = truncate_middle(&item.workspace_name, workspace_budget);
        let messages = if item.session.message_count == 1 {
            "message"
        } else {
            "messages"
        };
        let mut metadata = format!(
            " · {} · {} {messages}",
            item.session.updated_label(),
            item.session.message_count,
        );
        if row_width >= 60 && !item.session.model.trim().is_empty() {
            metadata.push_str(&format!(
                " · {}",
                item.session
                    .model
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        let metadata_budget = detail_budget.saturating_sub(cell_width(&workspace));
        ListItem::new(vec![
            title,
            Line::from(vec![
                Span::styled(
                    format!("{} ", crate::ui::icons::workspace()),
                    workspace_style,
                ),
                Span::styled(workspace, workspace_style),
                Span::styled(
                    truncate_end(&metadata, metadata_budget),
                    Style::default().fg(theme::muted()),
                ),
            ]),
        ])
    });
    let list = List::new(items)
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let mut state =
        ListState::default().with_selected(Some(app.popup_index.min(sessions.len() - 1)));
    frame.render_stateful_widget(list, rows[1], &mut state);
}

pub(crate) fn draw_reasoning(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(58, 46, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.state.is_streaming {
        " Reasoning · next response · ↑/↓ · Enter select · Esc close "
    } else {
        " Reasoning · ↑/↓ navigate · Enter select · Esc close "
    };
    let block = theme::modal_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let levels = reasoning_levels_for_model(&app.state.active_model);
    let items = levels.iter().map(|level| {
        let description = match *level {
            "auto" => "Yeet automatic reasoning policy for the selected model",
            "low" => "Faster, lighter reasoning",
            "medium" => "Balanced reasoning depth",
            "high" => "High reasoning depth",
            "xhigh" => "Extra-high reasoning where the selected model supports it",
            "max" => "Maximum provider-supported reasoning effort",
            _ => "Provider reasoning effort",
        };
        let marker = if *level == app.state.active_reasoning_level {
            "●"
        } else {
            " "
        };
        ListItem::new(Line::from(vec![
            Span::raw(format!("{marker} {level:<7}")),
            Span::styled(description, Style::default().fg(theme::muted())),
        ]))
    });
    let list = List::new(items)
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(list, inner, &mut state);
}
