//! Capability browser, status summaries, and detail dialog rendering.

use super::*;

fn capability_kind_label(kind: &str) -> &'static str {
    match kind {
        "builtin" => "BUILTIN",
        "capability" => "CAP",
        "skill" => "SKILL",
        "mcp" => "MCP",
        _ => "OTHER",
    }
}

fn capability_status(item: &CapabilityToggleItem) -> (&'static str, &'static str, Style) {
    if item.kind == "skill" {
        if item.enabled {
            (
                "●",
                "ATTACHED",
                Style::default()
                    .fg(theme::success())
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            ("◌", "LAZY", Style::default().fg(theme::accent_warm()))
        }
    } else if item.kind == "capability" || item.id == "builtin:skyline" {
        if item.enabled {
            (
                "●",
                "ATTACHED",
                Style::default()
                    .fg(theme::success())
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            ("○", "DETACHED", Style::default().fg(theme::muted()))
        }
    } else if item.enabled {
        (
            "●",
            "ON",
            Style::default()
                .fg(theme::success())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ("○", "OFF", Style::default().fg(theme::muted()))
    }
}

fn capability_behavior_hint(item: &CapabilityToggleItem) -> &'static str {
    match item.kind.as_str() {
        "skill" if item.enabled => {
            "Attached to this session. Space returns it to lazy on-demand discovery."
        }
        "skill" => {
            "Lazy skills stay discoverable on demand without occupying every request. Space attaches it to this session."
        }
        "capability" => "Space attaches or detaches this harness capability.",
        "mcp" => "Space enables or disables this MCP server for the project.",
        "builtin" if item.id == "builtin:skyline" => {
            "Space attaches or detaches Skyline for this session."
        }
        "builtin" => "Space enables or disables this built-in capability.",
        _ => "Space toggles this capability.",
    }
}

fn fit_cell(value: &str, width: usize) -> String {
    let value = truncate_end(value, width);
    let padding = width.saturating_sub(cell_width(&value));
    format!("{value}{}", " ".repeat(padding))
}

fn capability_summary(app: &App) -> String {
    let total = app.state.available_capabilities.len();
    let active = app
        .state
        .available_capabilities
        .iter()
        .filter(|item| item.enabled)
        .count();
    let skills = app
        .state
        .available_capabilities
        .iter()
        .filter(|item| item.kind == "skill")
        .count();
    let mcp = app
        .state
        .available_capabilities
        .iter()
        .filter(|item| item.kind == "mcp")
        .count();
    let bundled = app
        .state
        .available_capabilities
        .iter()
        .filter(|item| item.source.as_deref() == Some("bundled"))
        .count();
    let mut summary =
        format!("{total} available · {active} active/attached · {skills} skills · {mcp} MCP");
    if bundled > 0 {
        summary.push_str(&format!(" · {bundled} bundled"));
    }
    summary
}

fn capability_preview(item: &CapabilityToggleItem, streaming: bool) -> Text<'static> {
    let (marker, status, status_style) = capability_status(item);
    let source = item.source.as_deref().unwrap_or("runtime").to_owned();
    let mut lines = vec![
        Line::from(Span::styled(
            item.name.clone(),
            Style::default()
                .fg(theme::text())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Status  ", Style::default().fg(theme::muted())),
            Span::styled(format!("{marker} {status}"), status_style),
        ]),
        Line::from(vec![
            Span::styled("Type    ", Style::default().fg(theme::muted())),
            Span::raw(capability_kind_label(&item.kind)),
        ]),
        Line::from(vec![
            Span::styled("Source  ", Style::default().fg(theme::muted())),
            Span::raw(source),
        ]),
        Line::from(vec![
            Span::styled("ID      ", Style::default().fg(theme::muted())),
            Span::styled(item.id.clone(), Style::default().fg(theme::text_dim())),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Description",
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(item.description.clone()),
        Line::from(""),
        Line::from(Span::styled(
            capability_behavior_hint(item),
            Style::default().fg(theme::text_dim()),
        )),
    ];
    if streaming {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "Changes are locked while a response is running.",
            Style::default().fg(theme::accent_warm()),
        ));
    }
    Text::from(lines)
}

pub(in crate::tui::ui) fn draw_capabilities(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(92, 82, frame.area());
    theme::modal_backdrop(frame, area);
    let block = if app.state.is_streaming {
        theme::modal_block(
            " Capabilities · changes locked while response runs · ↑/↓ navigate · Enter details · Esc close ",
        )
    } else {
        theme::modal_block(
            " Capabilities · ↑/↓ navigate · type filter · Space toggle/attach · Enter details · Esc close ",
        )
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(1),
    ])
    .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Overview  ", Style::default().fg(theme::accent()).bold()),
            Span::styled(
                capability_summary(app),
                Style::default().fg(theme::text_dim()),
            ),
        ])),
        chunks[0],
    );

    let items = app.filtered_capabilities();
    let filter_text = if app.popup_filter.is_empty() {
        "all".to_owned()
    } else {
        app.popup_filter.clone()
    };
    let navigation_hint = if app.state.is_streaming {
        " · locked"
    } else if chunks[1].width >= 72 {
        " · ↑/↓ PgUp/PgDn · Ctrl-R refresh · Ctrl-U clear"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Search  ", Style::default().fg(theme::accent()).bold()),
            Span::styled(filter_text, Style::default().fg(theme::text())),
            Span::styled(
                format!(
                    " · shown {}/{}{}",
                    items.len(),
                    app.state.available_capabilities.len(),
                    navigation_hint
                ),
                Style::default().fg(theme::muted()),
            ),
        ])),
        chunks[1],
    );

    if app.state.is_loading_capabilities && app.state.available_capabilities.is_empty() {
        frame.render_widget(
            Paragraph::new("Loading capabilities…").fg(theme::text_dim()),
            chunks[2],
        );
        return;
    }

    if items.is_empty() {
        frame.render_widget(Paragraph::new("No matching capabilities."), chunks[2]);
        return;
    }

    let show_preview = chunks[2].width >= 92 && chunks[2].height >= 12;
    let (list_area, preview_area) = if show_preview {
        let columns = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(chunks[2]);
        (columns[0], Some(columns[1]))
    } else {
        (chunks[2], None)
    };

    let list_title = if app.popup_filter.is_empty() {
        format!("Available · {}", items.len())
    } else {
        format!("Matches · {}", items.len())
    };
    let list_block = theme::panel_block(list_title);
    let list_inner = list_block.inner(list_area);
    frame.render_widget(list_block, list_area);

    let row_width = usize::from(list_inner.width.saturating_sub(2));
    let status_width = 9usize;
    let kind_width = 7usize;
    let fixed = status_width + kind_width + 4;
    let name_width = row_width
        .saturating_sub(fixed)
        .min(if row_width >= 72 { 22 } else { 16 });
    let desc_width = row_width.saturating_sub(fixed + name_width + 1);
    let rows = items.iter().map(|item| {
        let (marker, status, status_style) = capability_status(item);
        let mut spans = vec![
            Span::styled(format!("{marker} "), status_style),
            Span::styled(fit_cell(status, status_width), status_style),
            Span::styled(
                fit_cell(capability_kind_label(&item.kind), kind_width),
                Style::default().fg(theme::accent()),
            ),
            Span::styled(
                fit_cell(&item.name, name_width),
                Style::default()
                    .fg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if desc_width > 0 {
            spans.push(Span::styled(
                truncate_end(&item.description, desc_width),
                Style::default().fg(theme::muted()),
            ));
        }
        ListItem::new(Line::from(spans))
    });
    let list = List::new(rows)
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let selected = app.popup_index.min(items.len().saturating_sub(1));
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, list_inner, &mut state);

    if let Some(preview_area) = preview_area {
        let preview_block = theme::panel_block("Selected");
        let preview_inner = preview_block.inner(preview_area);
        frame.render_widget(preview_block, preview_area);
        if let Some(item) = items.get(selected) {
            frame.render_widget(
                Paragraph::new(capability_preview(item, app.state.is_streaming))
                    .wrap(Wrap { trim: false }),
                preview_inner,
            );
        }
    }
}

pub(in crate::tui::ui) fn draw_capability_detail(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(80, 68, frame.area());
    theme::modal_backdrop(frame, area);
    let block = if app.state.is_streaming {
        theme::modal_block(
            " Capability detail · changes locked while response runs · Enter/Esc back ",
        )
    } else {
        theme::modal_block(" Capability detail · Space toggle/attach · Enter/Esc back ")
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(item) = app.capability_detail() else {
        frame.render_widget(Paragraph::new("Capability is no longer available."), inner);
        return;
    };

    frame.render_widget(
        Paragraph::new(capability_preview(item, app.state.is_streaming)).wrap(Wrap { trim: false }),
        inner,
    );
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    fn item(kind: &str, id: &str, enabled: bool, source: Option<&str>) -> CapabilityToggleItem {
        CapabilityToggleItem {
            id: id.into(),
            kind: kind.into(),
            name: "Example".into(),
            description: "Example capability".into(),
            enabled,
            source: source.map(str::to_owned),
        }
    }

    #[test]
    fn skill_status_distinguishes_lazy_from_disabled() {
        let lazy = item("skill", "skill:pdf", false, Some("bundled"));
        let attached = item("skill", "skill:pdf", true, Some("bundled"));
        assert_eq!(capability_status(&lazy).1, "LAZY");
        assert_eq!(capability_status(&attached).1, "ATTACHED");
        assert!(capability_behavior_hint(&lazy).contains("discoverable on demand"));
    }

    #[test]
    fn skyline_uses_attachment_status_semantics() {
        let detached = item("builtin", "builtin:skyline", false, Some("session"));
        let attached = item("builtin", "builtin:skyline", true, Some("session"));
        assert_eq!(capability_status(&detached).1, "DETACHED");
        assert_eq!(capability_status(&attached).1, "ATTACHED");
    }

    #[test]
    fn capability_cells_respect_terminal_width() {
        for width in 0..24 {
            let rendered = fit_cell("PDF界面-capability", width);
            assert!(cell_width(&rendered) <= width);
            if width > 0 && cell_width("PDF界面-capability") <= width {
                assert_eq!(cell_width(&rendered), width);
            }
        }
    }
}
