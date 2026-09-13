//! Consistent task state and persistent progress, independent of transcript scrolling.
use super::{
    format_elapsed, live_activity, live_operation, spinner_frame, theme, tool_step_counts,
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
            Self::Ready => theme::MUTED,
            Self::Working => theme::ACCENT_HOT,
            Self::Approval => theme::ACCENT_WARM,
            Self::Complete => theme::SUCCESS,
            Self::Failed => theme::ERROR,
            Self::Interrupted => theme::ACCENT,
        }
    }

    pub(super) fn marker(self, app: &App) -> &'static str {
        match self {
            Self::Ready => "○",
            Self::Working => spinner_frame(app.stream_elapsed().unwrap_or_default().as_millis()),
            Self::Approval => "!",
            Self::Complete => "✓",
            Self::Failed => "×",
            Self::Interrupted => "■",
        }
    }

    pub(super) fn badge(self, app: &App) -> Span<'static> {
        Span::styled(
            format!(" {} {} ", self.marker(app), self.label()),
            Style::default()
                .fg(self.color())
                .bg(theme::SURFACE_RAISED)
                .bold(),
        )
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
    let status = TaskStatus::for_app(app);
    if app.state.is_streaming
        || matches!(
            status,
            TaskStatus::Approval | TaskStatus::Failed | TaskStatus::Interrupted
        )
        || !app.follow_tail
    {
        2
    } else {
        1
    }
}

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let status = TaskStatus::for_app(app);
    let rail_color = if status == TaskStatus::Working {
        theme::pulse_color()
    } else {
        status.color()
    };
    let mut spans = vec![
        Span::styled("╸ ", Style::default().fg(rail_color)),
        status.badge(app),
    ];
    let (done, failed) = tool_step_counts(app);
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
    let suffix = if metrics.is_empty() {
        String::new()
    } else {
        format!("  //  {}", metrics.join("  ·  "))
    };
    let remaining = (area.width as usize).saturating_sub(Line::from(spans.clone()).width());
    spans.push(Span::styled(
        format!(" {}", fit(&suffix, remaining.saturating_sub(1))),
        Style::default().fg(theme::MUTED),
    ));

    let detail = if status == TaskStatus::Approval {
        "Waiting for your decision · Enter allow · Esc deny".to_owned()
    } else if status == TaskStatus::Working {
        if let Some(operation) = live_operation(app) {
            match operation.detail {
                Some(detail) => format!("{} · {detail}", operation.label),
                None => operation.label,
            }
        } else {
            let (title, detail) = live_activity(app);
            detail
                .map(|detail| format!("{title} · {detail}"))
                .unwrap_or(title)
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
        "Viewing history · Ctrl+End to return to latest".to_owned()
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(spans),
            Line::styled(
                format!("╰─ {}", fit(&detail, area.width.saturating_sub(3) as usize)),
                Style::default().fg(theme::MUTED),
            ),
        ]),
        area,
    );
}

#[cfg(test)]
mod tests;
