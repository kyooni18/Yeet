//! Consistent task state and persistent progress, independent of transcript scrolling.
use super::{
    activity_marker, activity_marker_color, animated_activity_label, format_elapsed, live_activity,
    live_operation, theme, tool_step_counts,
};
use crate::{
    app::App,
    model::{ConversationEntry, ConversationKind},
};
use ratatui::{Frame, layout::Rect, prelude::*, widgets::Paragraph};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskStatus {
    Ready,
    Working,
    Approval,
    Complete,
    Failed,
    Interrupted,
}

impl TaskStatus {
    pub(super) fn for_app(app: &App) -> Self {
        if app.state.pending_shell_permission.is_some()
            || app.state.pending_native_app_permission.is_some()
        {
            return Self::Approval;
        }
        if app.state.is_streaming {
            return Self::Working;
        }
        if app.state.error_message.is_some() {
            return Self::Failed;
        }
        latest_turn(app)
            .iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                ConversationKind::Activity { activity } => match activity.phase.as_str() {
                    Some("done") => Some(Self::Complete),
                    Some("failed") => Some(Self::Failed),
                    Some("interrupted") => Some(Self::Interrupted),
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or(Self::Ready)
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Working => "Working",
            Self::Approval => "Approval needed",
            Self::Complete => "Complete",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
        }
    }

    pub(super) fn color(self) -> Color {
        match self {
            Self::Ready => theme::muted(),
            Self::Working => theme::accent_hot(),
            Self::Approval => theme::warning(),
            Self::Complete => theme::success(),
            Self::Failed => theme::error(),
            Self::Interrupted => theme::muted(),
        }
    }

    pub(super) fn marker(self, app: &App) -> &'static str {
        match self {
            Self::Ready => "○",
            Self::Working => activity_marker(app),
            Self::Approval => "!",
            Self::Complete => "✓",
            Self::Failed => "×",
            Self::Interrupted => "—",
        }
    }
}

/// Older turns must not contribute failures or in-flight tools to this turn.
pub(super) fn latest_turn(app: &App) -> &[ConversationEntry] {
    let start = app
        .conversation
        .iter()
        .rposition(|entry| matches!(entry.kind, ConversationKind::User { .. }))
        .unwrap_or(0);
    &app.conversation[start..]
}

/// Elides single-line labels in terminal cells, including wide Unicode text.
pub(super) fn fit(value: &str, width: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if Span::raw(&value).width() <= width {
        return value;
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0;
    for ch in value.chars() {
        let cells = Span::raw(ch.to_string()).width();
        if used + cells > width - 1 {
            break;
        }
        result.push(ch);
        used += cells;
    }
    result.push('…');
    result
}

pub(super) fn height(app: &App) -> u16 {
    if matches!(
        TaskStatus::for_app(app),
        TaskStatus::Approval | TaskStatus::Failed | TaskStatus::Interrupted
    ) || (app.state.is_streaming && live_operation(app).is_some())
    {
        2
    } else {
        1
    }
}

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let status = TaskStatus::for_app(app);
    let rail_color = if status == TaskStatus::Working {
        activity_marker_color(app)
    } else {
        status.color()
    };
    let (done, failed) = tool_step_counts(app);
    let detail = if status == TaskStatus::Approval {
        "Enter allow · Esc deny · Ctrl+C stop".to_owned()
    } else if status == TaskStatus::Working {
        if let Some(operation) = live_operation(app) {
            let label = animated_activity_label(app, &operation.label);
            match operation.detail {
                Some(detail) => format!("{label} · {detail}"),
                None => label,
            }
        } else {
            let label = live_activity(app).0;
            animated_activity_label(app, &label)
        }
    } else if status == TaskStatus::Failed {
        app.state.error_message.clone().unwrap_or_else(|| {
            if failed > 0 {
                format!(
                    "{failed} tool step{} failed in the current turn",
                    if failed == 1 { "" } else { "s" }
                )
            } else {
                "Current turn failed".to_owned()
            }
        })
    } else if status == TaskStatus::Interrupted {
        "Current turn interrupted".to_owned()
    } else if !app.follow_tail && !app.conversation.is_empty() {
        if app.max_scroll > 0 {
            "Reading history"
        } else {
            "History Ctrl+End latest"
        }
        .to_owned()
    } else {
        String::new()
    };
    let detail =
        if status == TaskStatus::Working && !app.follow_tail && !app.conversation.is_empty() {
            if app.max_scroll > 0 {
                format!("History · {detail}")
            } else {
                format!("History Ctrl+End · {detail}")
            }
        } else {
            detail
        };
    let detail_row = matches!(
        status,
        TaskStatus::Approval | TaskStatus::Failed | TaskStatus::Interrupted
    ) || (status == TaskStatus::Working && live_operation(app).is_some());
    let detail_row_available = detail_row && area.height > 1;

    let mut metrics = Vec::new();
    if done > 0 {
        metrics.push(format!("{done} OK"));
    }
    if failed > 0 {
        metrics.push(format!("{failed} ERR"));
    }
    let elapsed = if app.state.is_streaming {
        app.stream_elapsed()
    } else {
        app.state
            .active_activity_entry_id
            .as_ref()
            .and_then(|_| app.latest_turn_duration())
    };
    if let Some(elapsed) = elapsed.filter(|_| status != TaskStatus::Ready) {
        metrics.push(format_elapsed(elapsed.as_millis()));
    }
    let metric_text = if metrics.is_empty() {
        String::new()
    } else {
        metrics.join(" · ")
    };
    let inline_detail = if detail_row_available || detail.is_empty() || detail == status.label() {
        String::new()
    } else {
        detail.clone()
    };
    let tail = match (inline_detail.is_empty(), metric_text.is_empty()) {
        (false, false) => format!("{metric_text} · {inline_detail}"),
        (false, true) => inline_detail,
        (true, false) => metric_text,
        (true, true) => String::new(),
    };

    let mut spans = vec![Span::styled(
        format!(" {} {} ", status.marker(app), status.label()),
        Style::default()
            .fg(rail_color)
            .bg(theme::surface_raised())
            .bold(),
    )];
    let activity = live_activity(app).0;
    let activity = animated_activity_label(app, &activity);
    if status == TaskStatus::Working && detail_row_available && activity != status.label() {
        let remaining = (area.width as usize)
            .saturating_sub(Line::from(spans.clone()).width())
            .saturating_sub(Span::raw(&tail).width() + 2);
        if remaining > 1 {
            spans.push(Span::styled(
                format!("  {}", fit(&activity, remaining.saturating_sub(2))),
                Style::default().fg(theme::text()),
            ));
        }
    }
    if !tail.is_empty() {
        let remaining = (area.width as usize).saturating_sub(Line::from(spans.clone()).width());
        spans.push(Span::styled(
            format!("  · {}", fit(&tail, remaining.saturating_sub(4))),
            Style::default().fg(theme::muted()),
        ));
    }

    let mut lines = vec![Line::from(spans)];
    if detail_row_available {
        lines.push(Line::styled(
            format!("  {}", fit(&detail, area.width.saturating_sub(2) as usize)),
            Style::default().fg(theme::muted()),
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}
