//! Existing app shell, composer, status, and overlay orchestration.
use super::{chrome, composer, dialogs, responsive, sessions, shell, status, task, theme};
use crate::app::{App, Mode};
use composer::draw as draw_input;
use dialogs::{
    draw_auth, draw_auth_key, draw_capabilities, draw_capability_detail, draw_goal, draw_help,
    draw_models, draw_permission, draw_provider_edit, draw_providers, draw_reasoning,
    draw_sandbox_policy, draw_sandbox_presets, draw_sessions, draw_settings, draw_settings_edit,
    draw_status_dialog,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Line, Span, Style},
    widgets::{Block, List, ListItem},
};
use status::draw as draw_status;

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    if app.mode == Mode::Files {
        super::files::draw(frame, app);
        return;
    }
    if app.state.is_streaming {
        let activity_label = task::live_operation(app)
            .map(|operation| operation.label)
            .unwrap_or_else(|| task::live_activity(app).0);
        app.sync_activity_label(&activity_label);
    }
    frame.render_widget(Block::default().style(theme::base()), frame.area());
    let adaptive = responsive::metrics(frame.area());
    let (sidebar_area, sidebar_targets) = shell::sidebar_session_targets(app, frame.area());
    app.set_sidebar_session_targets(sidebar_area, sidebar_targets);
    let area = shell::draw_shell(frame, app);
    let suggestions = app.command_suggestions();
    let requested_suggestion_height = if suggestions.is_empty() {
        0
    } else {
        (suggestions.len() as u16 + 2)
            .min(adaptive.suggestion_height)
            .min(area.height / 3)
    };
    let portrait = adaptive.shape == responsive::Shape::Portrait;
    let (input_inset, input_chrome) = if portrait { (4, 0) } else { (4, 2) };
    let input_rows = composer::layout(
        &app.input,
        app.cursor,
        area.width.saturating_sub(input_inset),
    );
    let input_height = (input_rows.lines.len() as u16)
        .clamp(adaptive.input_min_lines, adaptive.input_max_lines)
        + input_chrome;
    let task_height = task::height(app).min(adaptive.task_height);
    let suggestion_budget = area.height.saturating_sub(
        task_height
            .saturating_add(input_height)
            .saturating_add(adaptive.status_height)
            .saturating_add(1),
    );
    let suggestion_height = if requested_suggestion_height >= 3 && suggestion_budget >= 3 {
        requested_suggestion_height.min(suggestion_budget)
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(task_height),
            Constraint::Length(suggestion_height),
            Constraint::Length(input_height),
            Constraint::Length(adaptive.status_height),
        ])
        .split(area);

    sessions::draw(frame, app, chunks[0], adaptive.shape);
    if suggestion_height > 0 {
        draw_suggestions(frame, app, chunks[2], &suggestions);
    }
    task::draw(frame, app, chunks[1]);
    draw_input(frame, app, chunks[3]);
    draw_status(frame, app, chunks[4]);
    sessions::draw_context_menu(frame, app);

    match app.mode {
        Mode::Debate => dialogs::draw_debate(frame, app),
        Mode::Models => draw_models(frame, app),
        Mode::Reasoning => draw_reasoning(frame, app),
        Mode::Goal => draw_goal(frame, app),
        Mode::Sessions => draw_sessions(frame, app),
        Mode::Capabilities => draw_capabilities(frame, app),
        Mode::CapabilityDetail => draw_capability_detail(frame, app),
        Mode::Auth => draw_auth(frame, app),
        Mode::AuthKey => draw_auth_key(frame, app),
        Mode::Providers => draw_providers(frame, app),
        Mode::ProviderEdit => draw_provider_edit(frame, app),
        Mode::Settings => draw_settings(frame, app),
        Mode::SandboxPresets => draw_sandbox_presets(frame, app),
        Mode::SandboxPolicy => draw_sandbox_policy(frame, app),
        Mode::SettingsEdit => {
            draw_sandbox_policy(frame, app);
            draw_settings_edit(frame, app);
        }
        Mode::Status => draw_status_dialog(frame, app),
        Mode::Help => draw_help(frame),
        Mode::Chat | Mode::Files => {}
    }

    if app.state.pending_shell_permission.is_some()
        || app.state.pending_native_app_permission.is_some()
    {
        draw_permission(frame, app);
    }

    chrome::draw(frame, app);
}

fn draw_suggestions(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    suggestions: &[(String, String)],
) {
    let list = suggestion_list(suggestions);
    let mut state = ratatui::widgets::ListState::default().with_selected(Some(app.command_index));
    frame.render_stateful_widget(list, area, &mut state);
}

fn suggestion_list(suggestions: &[(String, String)]) -> List<'_> {
    let items = suggestions.iter().map(|(name, description)| {
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{name:<14}  "),
                Style::default().fg(theme::accent()),
            ),
            Span::styled(description.as_str(), Style::default().fg(theme::text_dim())),
        ]))
    });
    List::new(items)
        .block(theme::modal_block("Commands · ↑/↓ choose · Tab complete"))
        .highlight_style(theme::selected())
        .highlight_symbol("› ")
}

#[cfg(test)]
mod suggestion_tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        widgets::{ListState, StatefulWidget},
    };

    #[test]
    fn selection_highlights_the_whole_row_and_keeps_a_visible_marker() {
        let suggestions = vec![
            ("/help".into(), "Show help".into()),
            ("/models".into(), "Choose model".into()),
        ];
        let area = Rect::new(0, 0, 64, 4);
        let mut buffer = Buffer::empty(area);
        let mut state = ListState::default().with_selected(Some(1));
        StatefulWidget::render(suggestion_list(&suggestions), area, &mut buffer, &mut state);
        assert_eq!(buffer[(2, 2)].symbol(), "›");
        assert_eq!(buffer[(2, 1)].symbol(), " ");
        for x in 2..62 {
            assert_eq!(buffer[(x, 2)].bg, theme::selected_color(), "column {x}");
            assert_ne!(buffer[(x, 1)].bg, theme::selected_color());
        }
    }

    #[test]
    fn long_commands_have_a_gap_before_the_description_and_selection_scrolls() {
        let suggestions = vec![
            ("/help".into(), "Help".into()),
            ("/long-command-name".into(), "Description".into()),
        ];
        let area = Rect::new(0, 0, 64, 3);
        let mut buffer = Buffer::empty(area);
        let mut state = ListState::default().with_selected(Some(1));
        StatefulWidget::render(suggestion_list(&suggestions), area, &mut buffer, &mut state);
        let row: String = (0..area.width).map(|x| buffer[(x, 1)].symbol()).collect();
        assert!(row.contains("› /long-command-name  Description"), "{row}");
        assert_eq!(state.offset(), 1);
    }
}
