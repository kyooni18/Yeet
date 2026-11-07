use super::*;
use crate::{harness::tasks::Schedule, tui::app::tasks::Stage};

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}
pub(crate) fn draw_tasks(frame: &mut Frame<'_>, app: &mut App) {
    let Some(panel) = &mut app.task_manager else {
        return;
    };
    panel.sync(
        &app.state.scheduled_tasks,
        &app.state.tasks_notice,
        &app.state.tasks_response_id,
    );
    let area = centered_rect(96, 94, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" Tasks · Esc back ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(inner);
    let host = if app.state.tasks_scheduler_running {
        "Scheduler running · no OS wake/startup"
    } else {
        "Scheduler unavailable — reopen Yeet with background service"
    };
    frame.render_widget(Paragraph::new(format!("Scheduled agent runs · independent saved sessions\n{host}\nUnattended input/permission requests fail; existing sandbox applies.")).wrap(Wrap { trim: false }), chunks[0]);
    let status = if panel.pending {
        "Saving…"
    } else {
        panel
            .error
            .as_deref()
            .or(app.state.tasks_notice.as_deref())
            .unwrap_or("")
    };
    frame.render_widget(
        Paragraph::new(clean(status)).wrap(Wrap { trim: false }),
        chunks[2],
    );
    match &panel.stage {
        Stage::Browse => {
            let regions =
                Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(chunks[1]);
            let items: Vec<ListItem> = app
                .state
                .scheduled_tasks
                .iter()
                .map(|t| {
                    let schedule = match t.draft.schedule {
                        Schedule::At { at } => format!("at {}", at.to_rfc3339()),
                        Schedule::Interval { seconds } => format!("every {seconds}s"),
                        Schedule::QuotaReset => "next 5h quota reset (once)".into(),
                    };
                    let state = if t.running {
                        "RUNNING"
                    } else if t.enabled {
                        "enabled"
                    } else {
                        "paused/done"
                    };
                    ListItem::new(vec![
                        Line::from(clean(&format!("{} · {state} · {schedule}", t.draft.name))),
                        Line::from(clean(&format!(
                            "  {} · {}",
                            t.draft.folder.display(),
                            t.draft.model
                        ))),
                        Line::from(clean(&format!(
                            "  Next: {} · Last: {}",
                            t.next_run
                                .map(|at| at.to_rfc3339())
                                .unwrap_or_else(|| "—".into()),
                            t.last_result.as_deref().unwrap_or("not run")
                        ))),
                        Line::from(clean(&format!(
                            "  Session: {}",
                            t.session_id.as_deref().unwrap_or("—")
                        ))),
                        Line::from(""),
                    ])
                })
                .collect();
            if items.is_empty() {
                frame.render_widget(
                    Paragraph::new("No scheduled tasks. Press n to create one."),
                    regions[0],
                );
            } else {
                let mut state = ListState::default().with_selected(Some(panel.selected));
                frame.render_stateful_widget(
                    List::new(items)
                        .highlight_style(theme::selected())
                        .highlight_symbol("› "),
                    regions[0],
                    &mut state,
                );
            }
            frame.render_widget(Paragraph::new("n new · e/Enter edit · p pause/resume · d delete\nj/k/↑/↓ select · r refresh · q/Esc close").wrap(Wrap { trim: false }), regions[1]);
        }
        Stage::Edit => {
            let labels = [
                "Name",
                "Folder (Ctrl+B browse)",
                "Model (provider/model)",
                "Schedule",
                "Prompt",
            ];
            let regions = Layout::vertical([
                Constraint::Length(5),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(4),
            ])
            .split(chunks[1]);
            let lines: Vec<Line> = labels
                .iter()
                .enumerate()
                .map(|(i, label)| {
                    let marker = if panel.field == i { "›" } else { " " };
                    let preview = clean(&panel.fields[i]).replace('\n', " ↵ ");
                    let line = Line::from(truncate_end(
                        &format!("{marker} {label}: {preview}"),
                        regions[0].width as usize,
                    ));
                    if panel.field == i {
                        line.style(theme::selected())
                    } else {
                        line
                    }
                })
                .collect();
            frame.render_widget(Paragraph::new(lines), regions[0]);
            frame.render_widget(
                Paragraph::new(format!(
                    "Editing {} (append / Backspace)",
                    labels[panel.field]
                )),
                regions[1],
            );
            let value = Paragraph::new(format!("{}▏", clean(&panel.fields[panel.field])))
                .wrap(Wrap { trim: false });
            let scroll = value
                .line_count(regions[2].width)
                .saturating_sub(regions[2].height as usize)
                .min(u16::MAX as usize) as u16;
            frame.render_widget(value.scroll((scroll, 0)), regions[2]);
            frame.render_widget(Paragraph::new("Schedule: every SECONDS | at RFC3339 | quota (next 5h reset, once)\nTab/Shift+Tab field · Ctrl+U clear · Enter newline in prompt\nFolder: Ctrl+B browse · Ctrl+S save · Esc cancel").wrap(Wrap { trim: false }), regions[3]);
        }
        Stage::Folder => {
            let regions = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(chunks[1]);
            frame.render_widget(
                Paragraph::new(clean(&panel.folder.display().to_string()))
                    .wrap(Wrap { trim: false }),
                regions[0],
            );
            let mut state = ListState::default().with_selected(Some(panel.directory_index));
            let items: Vec<_> = panel
                .directories
                .iter()
                .map(|name| ListItem::new(format!("{}/", clean(name))))
                .collect();
            frame.render_stateful_widget(
                List::new(items)
                    .highlight_style(theme::selected())
                    .highlight_symbol("› "),
                regions[1],
                &mut state,
            );
            frame.render_widget(Paragraph::new("Enter descend · Backspace parent · Ctrl+S choose folder\n↑/↓ or j/k select · Esc return without choosing").wrap(Wrap { trim: false }), regions[2]);
        }
        Stage::Delete(id) => {
            frame.render_widget(Paragraph::new(format!("Delete task {}?\nRunning tasks cannot be deleted. Saved sessions are kept.\nType delete and Enter, or Esc to cancel.\n\n{}", clean(id), clean(&panel.text))).wrap(Wrap { trim: false }), chunks[1]);
        }
    }
}
