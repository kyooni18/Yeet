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
            Span::styled(" FILTER// ", Style::default().fg(theme::ACCENT_WARM).bold()),
            Span::styled(query, Style::default().fg(theme::TEXT)),
            Span::styled(
                format!(
                    "  ·  {} MODELS  ·  {provider_count} PROVIDERS{refreshing}{navigation_hint}",
                    rows.len()
                ),
                Style::default().fg(theme::MUTED),
            ),
        ])),
        chunks[0],
    );

    if app.state.is_loading_models && rows.is_empty() {
        frame.render_widget(
            Paragraph::new("Loading model catalog…").fg(theme::TEXT_DIM),
            chunks[1],
        );
        return;
    }
    if rows.is_empty() {
        let (message, color) = app
            .state
            .error_message
            .as_deref()
            .map(|message| (message, theme::ERROR))
            .unwrap_or((
                "No matching models. Search by provider or model.",
                theme::MUTED,
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
                Style::default().fg(theme::TEXT),
            ),
            Span::styled(context, Style::default().fg(theme::MUTED)),
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
                    " GOAL// ",
                    Style::default().fg(theme::ACCENT_WARM).bold(),
                ),
                Span::styled(status, Style::default().fg(theme::TEXT).bold()),
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
            Span::styled(description, Style::default().fg(theme::TEXT)),
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
            Span::styled(" FILTER// ", Style::default().fg(theme::ACCENT_WARM).bold()),
            Span::styled(filter_label, Style::default().fg(theme::TEXT)),
            Span::styled(
                format!(
                    "  ·  {}/{}  ·  ↑/↓ PgUp/PgDn",
                    sessions.len(),
                    all_sessions.len()
                ),
                Style::default().fg(theme::MUTED),
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
                    Style::default().fg(theme::MUTED),
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
                    Style::default().fg(theme::MUTED),
                ),
            ]),
            rows[1],
        );
        return;
    }

    let row_width = rows[1].width.saturating_sub(2) as usize;
    let items = sessions.iter().map(|item| {
        let marker = if app.state.current_session_id.as_deref() == Some(item.session.id.as_str()) {
            "●"
        } else {
            " "
        };
        let workspace_style = if item.workspace_current {
            Style::default().fg(theme::ACCENT_WARM).bold()
        } else {
            Style::default().fg(theme::MUTED)
        };
        let metadata = if row_width >= 80 {
            format!(
                "  ·  {}  ·  {} msg",
                truncate_middle(&item.session.model, 18),
                item.session.message_count
            )
        } else if row_width >= 60 {
            format!(
                "  ·  {}  ·  {} msg",
                truncate_middle(&item.session.model, 10),
                item.session.message_count
            )
        } else {
            String::new()
        };
        let marker_width = 2;
        let label_budget = row_width.saturating_sub(marker_width + cell_width(&metadata));
        let mut spans = vec![Span::raw(format!("{marker} "))];
        if label_budget >= 20 {
            let max_workspace = label_budget.saturating_sub(13);
            let workspace_budget = max_workspace.clamp(6, 14);
            let title_budget = label_budget.saturating_sub(workspace_budget + 3);
            spans.push(Span::styled(
                truncate_middle(&item.workspace_name, workspace_budget),
                workspace_style,
            ));
            spans.push(Span::styled(" / ", Style::default().fg(theme::BORDER)));
            spans.push(Span::styled(
                truncate_middle(&item.session.title, title_budget),
                Style::default().fg(theme::TEXT),
            ));
        } else {
            spans.push(Span::styled(
                truncate_middle(&item.session.title, label_budget),
                Style::default().fg(theme::TEXT),
            ));
        }
        if !metadata.is_empty() {
            spans.push(Span::styled(metadata, Style::default().fg(theme::MUTED)));
        }
        ListItem::new(Line::from(spans))
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

    let descriptions = [
        "Yeet default: low reasoning for ordinary agent requests",
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
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(list, inner, &mut state);
}
