//! Consistent task state and persistent progress, independent of transcript scrolling.
use super::sessions::tools::{reasoning_summary_items, tool_activity_summary, tool_activity_title};
use super::{markdown::markdown_lines, text::format_elapsed, theme};
use crate::{
    app::App,
    model::{ConversationEntry, ConversationKind, ToolCallStatus},
};
use ratatui::{
    Frame,
    layout::Rect,
    prelude::*,
    widgets::{Block, Paragraph},
};

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
    if !app.follow_tail && !app.conversation.is_empty() {
        return 1;
    }
    match TaskStatus::for_app(app) {
        TaskStatus::Approval | TaskStatus::Failed | TaskStatus::Interrupted => 2,
        TaskStatus::Working => match live_operation(app) {
            Some(operation) if operation.detail.is_some() => 2,
            Some(_) => 1,
            None => 0,
        },
        TaskStatus::Ready | TaskStatus::Complete => 0,
    }
}

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Block::default().style(theme::surface()), area);
    let status = TaskStatus::for_app(app);
    let rail_color = if status == TaskStatus::Working {
        activity_marker_color(app)
    } else {
        status.color()
    };
    let (done, failed, active_tools) = tool_step_counts(app);
    let detail = if status == TaskStatus::Approval {
        "Enter allow · Esc deny · Ctrl+C stop".to_owned()
    } else if status == TaskStatus::Working {
        if let Some(operation) = live_operation(app) {
            operation.detail.unwrap_or_default()
        } else {
            String::new()
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
    if active_tools > 0 {
        metrics.push(format!("{active_tools} RUN"));
    }
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

    let headline = if status == TaskStatus::Working {
        live_operation(app)
            .map(|operation| animated_activity_label(app, &operation.label))
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| status.label().to_owned())
    } else {
        status.label().to_owned()
    };
    let marker = format!(" {} ", status.marker(app));
    let headline_width = (area.width as usize)
        .saturating_sub(Span::raw(&marker).width())
        .saturating_sub(if tail.is_empty() {
            0
        } else {
            Span::raw(&tail).width() + 3
        });
    let mut spans = vec![
        Span::styled(
            marker,
            Style::default()
                .fg(rail_color)
                .bg(theme::surface_raised())
                .bold(),
        ),
        Span::styled(
            fit(&headline, headline_width.max(1)),
            Style::default().fg(theme::text()).bold(),
        ),
    ];
    if !tail.is_empty() {
        let remaining = (area.width as usize).saturating_sub(Line::from(spans.clone()).width());
        if remaining > 3 {
            spans.push(Span::styled(
                format!("  · {}", fit(&tail, remaining.saturating_sub(4))),
                Style::default().fg(theme::muted()),
            ));
        }
    }

    let mut lines = vec![Line::from(spans)];
    if detail_row_available {
        lines.push(Line::from(vec![
            Span::styled("  ╰ ", Style::default().fg(theme::border_dim())),
            Span::styled(
                fit(&detail, area.width.saturating_sub(4) as usize),
                Style::default().fg(theme::muted()),
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

pub(super) fn live_activity(app: &App) -> (String, Option<&str>) {
    if app.state.active_reasoning_entry_id.is_some() {
        let title = app
            .state
            .active_activity_entry_id
            .as_deref()
            .and_then(|id| {
                app.conversation.iter().rev().find_map(|entry| {
                    (entry.id == id).then(|| match &entry.kind {
                        ConversationKind::Activity { activity } => activity.title.clone(),
                        _ => String::new(),
                    })
                })
            })
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "Reasoning".into());
        return (title, None);
    }
    let Some(id) = app.state.active_activity_entry_id.as_deref() else {
        return ("Working".into(), None);
    };
    app.conversation
        .iter()
        .rev()
        .find(|entry| entry.id == id)
        .and_then(|entry| match &entry.kind {
            ConversationKind::Activity { activity } => {
                Some((activity.title.clone(), activity.detail.as_deref()))
            }
            _ => None,
        })
        .unwrap_or_else(|| ("Working".into(), None))
}

pub(super) fn animated_activity_label(app: &App, label: &str) -> String {
    let Some(started_at) = app.activity_label_started_at else {
        return label.to_owned();
    };
    let progress = (started_at.elapsed().as_millis() as f32
        / f32::from(app.activity_label_transition_ms))
    .clamp(0.0, 1.0);
    if progress >= 1.0 || app.activity_label_to != label {
        return label.to_owned();
    }
    morph_activity_text(&app.activity_label_from, label, progress)
}

fn morph_activity_text(from: &str, to: &str, progress: f32) -> String {
    let from = from.chars().collect::<Vec<_>>();
    let to = to.chars().collect::<Vec<_>>();
    let length = from.len().max(to.len());
    let mut result = String::new();
    for index in 0..length {
        let stagger = index as f32 / (length.max(1) as f32) * 0.35;
        let local = ((progress - stagger) / 0.65).clamp(0.0, 1.0);
        let character = if local < 0.5 {
            from.get(index).copied().unwrap_or(' ')
        } else {
            to.get(index).copied().unwrap_or(' ')
        };
        result.push(character);
    }
    result.trim_end().to_owned()
}

fn tool_step_counts(app: &App) -> (usize, usize, usize) {
    let mut counts = (0usize, 0usize, 0usize);
    for entry in latest_turn(app) {
        let calls: &[crate::model::ConversationToolCall] = match &entry.kind {
            ConversationKind::ToolCall { tool_call } => std::slice::from_ref(tool_call),
            ConversationKind::Assistant { tool_calls, .. } => tool_calls.as_slice(),
            _ => continue,
        };
        for call in calls {
            match call.status {
                ToolCallStatus::Completed => counts.0 += 1,
                ToolCallStatus::Failed
                | ToolCallStatus::Cancelled
                | ToolCallStatus::Interrupted
                | ToolCallStatus::TimedOut => counts.1 += 1,
                ToolCallStatus::Preparing
                | ToolCallStatus::AwaitingPermission
                | ToolCallStatus::Running => counts.2 += 1,
                ToolCallStatus::Suppressed => {}
            }
        }
    }
    counts
}

/// A deliberately small motion cue for the one piece of UI that is still live.
/// The glyph family changes with the actual live phase, while the cadence stays stable.
pub(super) fn activity_marker(app: &App) -> &'static str {
    let frame = activity_frame(app);
    if app.state.active_reasoning_entry_id.is_some() {
        ["—", "–", "-", "·", "∙", "○", "◌", "◉", "◌", "○", "∙", "·"][frame]
    } else if app.state.active_assistant_entry_id.is_some() {
        ["-", "–", "—", "·", "•", "●", "◉", "●", "•", "·", "–", "-"][frame]
    } else {
        ["-", "–", "—", "◌", "◍", "◉", "◍", "◌", "—", "–", "-", "·"][frame]
    }
}

pub(super) fn activity_marker_color(app: &App) -> Color {
    let frame = activity_frame(app);
    if app.state.active_reasoning_entry_id.is_some() {
        [
            theme::muted(),
            theme::muted(),
            theme::muted(),
            theme::accent_warm(),
            theme::accent_warm(),
            theme::accent_hot(),
            theme::accent_hot(),
            theme::accent_warm(),
            theme::accent_warm(),
            theme::muted(),
            theme::muted(),
            theme::muted(),
        ][frame]
    } else if app.state.active_assistant_entry_id.is_some() {
        [
            theme::user(),
            theme::text_dim(),
            theme::text(),
            theme::text_dim(),
            theme::text(),
            theme::text_dim(),
            theme::user(),
            theme::user(),
            theme::text_dim(),
            theme::user(),
            theme::text_dim(),
            theme::user(),
        ][frame]
    } else {
        [
            theme::accent(),
            theme::accent(),
            theme::accent_hot(),
            theme::accent_hot(),
            theme::accent_hot(),
            theme::accent(),
            theme::accent(),
            theme::accent_hot(),
            theme::accent_hot(),
            theme::accent(),
            theme::accent(),
            theme::accent(),
        ][frame]
    }
}

fn activity_frame(app: &App) -> usize {
    let elapsed_ms = app
        .stream_elapsed()
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    ((elapsed_ms / 110) as usize) % 12
}

pub(super) struct LiveOperation {
    pub(super) label: String,
    pub(super) detail: Option<String>,
}

pub(super) fn live_operation(app: &App) -> Option<LiveOperation> {
    if app.state.active_reasoning_entry_id.is_some() {
        return Some(LiveOperation {
            label: live_activity(app).0,
            detail: live_reasoning_detail(app),
        });
    }

    if let Some(call) = latest_turn(app)
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            ConversationKind::ToolCall { tool_call }
                if matches!(
                    tool_call.status,
                    ToolCallStatus::Preparing
                        | ToolCallStatus::AwaitingPermission
                        | ToolCallStatus::Running
                ) =>
            {
                Some(tool_call)
            }
            ConversationKind::Assistant { tool_calls, .. } => {
                tool_calls.iter().rev().find(|call| {
                    matches!(
                        call.status,
                        ToolCallStatus::Preparing
                            | ToolCallStatus::AwaitingPermission
                            | ToolCallStatus::Running
                    )
                })
            }
            _ => None,
        })
    {
        return Some(LiveOperation {
            label: tool_activity_title(call),
            detail: tool_activity_summary(call),
        });
    }

    if app.state.active_assistant_entry_id.is_some() {
        return Some(LiveOperation {
            label: "Responding".into(),
            detail: None,
        });
    }

    app.state.is_streaming.then(|| {
        let (label, detail) = live_activity(app);
        LiveOperation {
            label,
            detail: detail.map(str::to_owned),
        }
    })
}

fn live_reasoning_detail(app: &App) -> Option<String> {
    let id = app.state.active_reasoning_entry_id.as_deref()?;
    if !app.state.active_reasoning_summary.trim().is_empty()
        && let Some(item) = reasoning_summary_items(&app.state.active_reasoning_summary).last()
    {
        let plain = markdown_plain_text(item);
        if !plain.is_empty() {
            return Some(plain);
        }
    }
    if let Some(line) = app
        .state
        .active_reasoning_text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
    {
        let plain = markdown_plain_text(line.trim());
        if !plain.is_empty() {
            return Some(plain);
        }
    }
    app.conversation.iter().rev().find_map(|entry| {
        if entry.id != id {
            return None;
        }
        match &entry.kind {
            ConversationKind::Reasoning { content, summary } => summary
                .as_deref()
                .and_then(|value| reasoning_summary_items(value).last().cloned())
                .map(|value| markdown_plain_text(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    content
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .map(|line| markdown_plain_text(line.trim()))
                        .filter(|value| !value.is_empty())
                }),
            _ => None,
        }
    })
}

fn markdown_plain_text(content: &str) -> String {
    markdown_lines(content)
        .into_iter()
        .map(|line| line.to_string())
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod animation_tests {
    use super::morph_activity_text;

    #[test]
    fn activity_text_morphs_between_live_phases() {
        assert_eq!(
            morph_activity_text("Thinking", "Searching", 0.0),
            "Thinking"
        );
        assert_eq!(
            morph_activity_text("Thinking", "Searching", 1.0),
            "Searching"
        );
        let middle = morph_activity_text("Thinking", "Searching", 0.5);
        assert_ne!(middle, "Thinking");
        assert_ne!(middle, "Planning");
    }
}
