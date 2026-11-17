//! Shared transcript controls and semantic view descriptions.
use crate::model::{ConversationEntry, ConversationToolCall, ModelActivity};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationIcon {
    Terminal,
    Edit,
    File,
    Search,
    Folder,
    Screen,
    Web,
    Mcp,
    Skill,
    Tool,
    Reasoning,
    Info,
    Error,
    Copy,
    Refresh,
    Activity,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailSection {
    pub title: String,
    pub content: String,
    pub monospaced: bool,
    pub is_error: bool,
    pub has_background: bool,
}
/// Tool-specific semantic detail. Data only: hosts decide how to paint it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RichDetail {
    Diff(DiffDetail),
    Terminal(TerminalDetail),
    Source(SourceDetail),
    Diagnostics { items: Vec<DiagnosticItem> },
    Notice { message: String, is_error: bool },
}
/// Where complete data lives when the tool returned only a bounded preview.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Externalized {
    pub artifact_id: Option<String>,
    pub bytes: Option<u64>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffDetail {
    pub files: Vec<DiffFile>,
    /// Set when the edit payload was stored as an artifact, so no hunks are held here.
    pub externalized: Option<Externalized>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffFile {
    pub path: String,
    pub previous_path: Option<String>,
    /// `create`, `update`, `delete`, `rename`, or the backend's own word.
    pub operation: String,
    pub hunks: Vec<DiffHunk>,
    pub warnings: Vec<String>,
    /// False when the call has no recorded result (pending/failed) or it was externalized.
    pub has_diff: bool,
    pub truncated: bool,
    /// Added and removed line counts of the hunks held here, for hosts that
    /// show them without walking every line.
    #[serde(default)]
    pub added: usize,
    #[serde(default)]
    pub removed: usize,
}
impl DiffFile {
    pub fn added(&self) -> usize {
        self.count(DiffLineKind::Added)
    }
    pub fn removed(&self) -> usize {
        self.count(DiffLineKind::Removed)
    }
    /// Fills the serialized counts once the hunks are final.
    pub fn with_counts(mut self) -> Self {
        self.added = self.added();
        self.removed = self.removed();
        self
    }
    fn count(&self, kind: DiffLineKind) -> usize {
        self.hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind == kind)
            .count()
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_end: Option<usize>,
    pub new_start: usize,
    pub new_end: Option<usize>,
    pub lines: Vec<DiffLine>,
    pub truncated: bool,
    /// Unified-diff header for the recorded ranges: `@@ -4,3 +4,5 @@`.
    #[serde(default)]
    pub header: String,
}
impl DiffHunk {
    pub fn unified_header(&self) -> String {
        let range = |start: usize, end: Option<usize>| match end {
            Some(end) => format!("{start},{}", (end + 1).saturating_sub(start)),
            None => start.to_string(),
        };
        format!(
            "@@ -{} +{} @@",
            range(self.old_start, self.old_end),
            range(self.new_start, self.new_end)
        )
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    /// Backend marker where the middle of a long hunk was dropped.
    Omitted,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticItem {
    pub severity: String,
    pub path: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub message: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    Waiting,
    Running,
    /// The command outlived its foreground window and continues as a job.
    BackgroundJob,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Interrupted,
}
impl TerminalState {
    /// Sentence-case state copy shared by every host.
    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting for execution",
            Self::Running => "Running",
            Self::BackgroundJob => "Running in background",
            Self::Succeeded => "Finished",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::TimedOut => "Timed out",
            Self::Interrupted => "Interrupted",
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputStream {
    pub text: String,
    /// Total bytes the process wrote, when reported.
    pub bytes: Option<u64>,
    /// The backend dropped part of this stream.
    pub truncated: bool,
    /// Only the latest bytes of a still-running command.
    pub tail: bool,
}
/// Streams are kept apart because their relative order is not recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalDetail {
    pub working_directory: Option<String>,
    pub command: Option<String>,
    pub state: TerminalState,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<u64>,
    pub stdout: Option<OutputStream>,
    pub stderr: Option<OutputStream>,
    /// Output whose stream is unknown (text-only or older results).
    pub combined: Option<String>,
    pub job_id: Option<String>,
    pub artifact_id: Option<String>,
    /// The visible output is a window onto a larger transcript.
    pub windowed: bool,
    /// State, exit code and duration as one caption: `Failed · exit 2 · 1.4 s`.
    #[serde(default)]
    pub status: String,
}
impl TerminalDetail {
    pub fn status_caption(&self) -> String {
        let mut parts = vec![self.state.label().to_owned()];
        if let Some(code) = self.exit_code {
            parts.push(format!("exit {code}"));
        }
        if let Some(duration) = self.duration_ms {
            parts.push(format_duration_ms(duration));
        }
        parts.join(" · ")
    }
}
/// `820 ms` below one second, `1.4 s` above.
pub fn format_duration_ms(duration: u64) -> String {
    if duration < 1000 {
        format!("{duration} ms")
    } else {
        format!("{:.1} s", duration as f64 / 1000.0)
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceDetail {
    pub files: Vec<SourceFile>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceAvailability {
    Lines,
    /// Externalized read: only a bounded prefix of the range is present.
    Preview,
    /// No lines were recorded (still running, or metadata only).
    Pending,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub total_lines: Option<usize>,
    pub next_start_line: Option<usize>,
    pub availability: SourceAvailability,
    pub lines: Vec<SourceLine>,
    pub artifact_id: Option<String>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceLine {
    pub number: usize,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolView {
    pub raw_tool: ConversationToolCall,
    pub title: String,
    pub summary: Option<String>,
    pub status_label: Option<String>,
    pub metadata: Option<String>,
    pub active: bool,
    pub failed: bool,
    pub awaits_permission: bool,
    pub icon: ConversationIcon,
    pub details: Vec<DetailSection>,
    /// Specialized presentation; hosts without one keep using `details`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rich: Vec<RichDetail>,
    /// Arguments and results behind `rich`, for secondary inspection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw: Vec<DetailSection>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceView {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub status_label: Option<String>,
    pub metadata: Option<String>,
    pub icon: ConversationIcon,
    pub active: bool,
    pub expanded: bool,
    pub details: Vec<DetailSection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rich: Vec<RichDetail>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw: Vec<DetailSection>,
    #[serde(default)]
    pub raw_expanded: bool,
    /// Text for the row: `title`, or "Working" when the provider only sent a
    /// count or a generic phase.
    #[serde(default)]
    pub label: String,
    /// Single-line object of the action (path, command, query), when distinct.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Exceptional state worth visible copy; routine success has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<TraceAttention>,
    /// Whether opening the row reveals anything.
    #[serde(default)]
    pub has_details: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// Waiting on the user; not an error and not progress.
    Approval,
    /// Failed, denied, timed out, cancelled or interrupted.
    Problem,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceAttention {
    pub label: String,
    pub kind: AttentionKind,
}
/// Some providers report only a count and a generic phase ("6 · Running").
/// That is not a description of work and is never shown as an action.
fn is_opaque_work_label(label: &str) -> bool {
    const PHASES: [&str; 5] = ["running", "working", "preparing", "pending", "in progress"];
    const UNITS: [&str; 9] = [
        "action",
        "actions",
        "activities",
        "tool",
        "tools",
        "call",
        "calls",
        "event",
        "events",
    ];
    label
        .split(['·', '•', '.'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .all(|part| {
            let part = part.to_ascii_lowercase();
            if PHASES.contains(&part.as_str()) {
                return true;
            }
            let digits = part.trim_start_matches(|ch: char| ch.is_ascii_digit());
            digits.len() < part.len() && (digits.is_empty() || UNITS.contains(&digits.trim()))
        })
}
impl TraceView {
    /// Derives the presentation fields from the semantic ones. Called once
    /// per projection, after every adapter-specific adjustment.
    pub fn finish(&mut self) {
        let summary = self
            .summary
            .as_deref()
            .filter(|summary| !summary.is_empty() && *summary != self.title);
        let full = match summary {
            Some(summary) => format!("{} · {summary}", self.title),
            None => self.title.clone(),
        };
        let opaque = is_opaque_work_label(&full);
        self.context = (!opaque).then(|| summary.map(str::to_owned)).flatten();
        self.label = if opaque {
            "Working".into()
        } else {
            self.title.clone()
        };
        self.attention = self.attention_label().map(|label| {
            let lower = label.to_ascii_lowercase();
            TraceAttention {
                label: label.to_owned(),
                kind: if lower.contains("approv") || lower.contains("permission") {
                    AttentionKind::Approval
                } else {
                    AttentionKind::Problem
                },
            }
        });
        self.has_details =
            !self.details.is_empty() || self.metadata.is_some() || !self.rich.is_empty();
    }
    /// Stable ID of the raw-inspection disclosure shown beneath a rich view.
    pub fn raw_id(&self) -> Option<String> {
        (!self.rich.is_empty() && !self.raw.is_empty()).then(|| format!("raw:{}", self.id))
    }
    /// Only exceptional states need visible status copy beside the action.
    pub fn attention_label(&self) -> Option<&str> {
        self.status_label.as_deref().filter(|label| {
            let label = label.to_ascii_lowercase();
            [
                "permission",
                "approv",
                "fail",
                "error",
                "denied",
                "timed",
                "cancel",
                "interrupt",
            ]
            .iter()
            .any(|word| label.contains(word))
        })
    }
}
// Boxing these variants would churn many call sites for little benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActivityEvent {
    Activity {
        id: String,
        key: String,
        activity: ModelActivity,
        is_active: bool,
        trace: TraceView,
    },
    Tool {
        key: String,
        tool: ConversationToolCall,
        view: ToolView,
        trace: TraceView,
    },
    Skill {
        id: String,
        key: String,
        name: String,
        content: String,
        status: Option<String>,
        trace: TraceView,
    },
    Mcp {
        id: String,
        key: String,
        server: String,
        name: String,
        content: String,
        is_error: bool,
        trace: TraceView,
    },
}
impl ActivityEvent {
    pub fn key(&self) -> &str {
        match self {
            Self::Activity { key, .. }
            | Self::Tool { key, .. }
            | Self::Skill { key, .. }
            | Self::Mcp { key, .. } => key,
        }
    }
    pub fn trace(&self) -> &TraceView {
        match self {
            Self::Activity { trace, .. }
            | Self::Tool { trace, .. }
            | Self::Skill { trace, .. }
            | Self::Mcp { trace, .. } => trace,
        }
    }
    pub fn trace_mut(&mut self) -> &mut TraceView {
        match self {
            Self::Activity { trace, .. }
            | Self::Tool { trace, .. }
            | Self::Skill { trace, .. }
            | Self::Mcp { trace, .. } => trace,
        }
    }
    pub fn failed(&self) -> bool {
        match self {
            Self::Tool { view, .. } => view.failed,
            Self::Mcp { is_error, .. } => *is_error,
            _ => false,
        }
    }
    pub fn awaits_permission(&self) -> bool {
        matches!(self,Self::Tool{view,..} if view.awaits_permission)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityGroup {
    pub id: String,
    pub events: Vec<ActivityEvent>,
    pub summary: String,
    /// Compact exceptional-state copy, projected once for all adapters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_label: Option<String>,
    pub active: bool,
    pub failed: bool,
    pub awaits_permission: bool,
    pub expanded: bool,
}
impl ActivityGroup {
    /// A singleton is one disclosure, not a group wrapped around the same row.
    /// Keep the group's stable ID so expansion survives appended tool events.
    pub fn single_trace(&self) -> Option<TraceView> {
        if self.events.len() != 1 {
            return None;
        }
        let mut trace = self.events[0].trace().clone();
        trace.id = self.id.clone();
        trace.expanded = self.expanded;
        Some(trace)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ConversationAction {
    Toggle(String),
    Select(Option<String>),
    MoveSelection(isize),
    SetExpandAll(bool),
    SetInspectWork(bool),
    Copy(String),
    Edit(String),
    Regenerate(String),
    Reset,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageControl {
    pub label: String,
    pub icon: ConversationIcon,
    pub action: ConversationAction,
    pub enabled: bool,
}
// Boxing these variants would churn many call sites for little benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DisplayItem {
    Entry {
        id: String,
        entry: ConversationEntry,
        controls: Vec<MessageControl>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<PromptDisclosure>,
    },
    Activity {
        id: String,
        group: ActivityGroup,
    },
    Reasoning {
        id: String,
        content: String,
        summary: Option<String>,
        is_active: bool,
        trace: TraceView,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptDisclosure {
    pub preview: String,
    pub expanded: bool,
}
impl DisplayItem {
    pub fn id(&self) -> &str {
        match self {
            Self::Entry { id, .. } | Self::Activity { id, .. } | Self::Reasoning { id, .. } => id,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationView {
    pub items: Vec<DisplayItem>,
    pub last_user_id: Option<String>,
    pub last_assistant_id: Option<String>,
    pub streaming_text: String,
    pub streaming: bool,
    pub preparing: bool,
    pub preparing_label: String,
    pub error: Option<String>,
    pub selected: Option<String>,
}
impl ConversationView {
    /// Visible work disclosures, in the same order as native row geometry.
    pub fn work_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for item in &self.items {
            match item {
                DisplayItem::Entry {
                    id,
                    prompt: Some(_),
                    ..
                } => ids.push(id.clone()),
                DisplayItem::Entry { .. } => {}
                DisplayItem::Reasoning { .. } => {}
                DisplayItem::Activity { id, group } => {
                    // Headerless groups expose each call regardless of the
                    // legacy aggregate expansion flag.
                    if group.events.len() > 1 {
                        for event in group.events.iter().filter(|event| {
                            !event.trace().details.is_empty() || event.trace().metadata.is_some()
                        }) {
                            ids.push(event.key().to_owned());
                            if event.trace().expanded {
                                ids.extend(event.trace().raw_id());
                            }
                        }
                    } else if let Some(trace) = group.single_trace() {
                        ids.push(id.clone());
                        if trace.expanded {
                            ids.extend(trace.raw_id());
                        }
                    }
                }
            }
        }
        ids
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationEffect {
    pub command: Option<crate::harness::HarnessCommand>,
    pub copy: Option<CopyEffect>,
    pub edit_draft: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyEffect {
    pub entry_id: String,
    pub text: String,
}

/// Native clipboard/editor effects; execution commands remain host-internal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationUiEffect {
    pub copy: Option<CopyEffect>,
    pub edit_draft: Option<String>,
}
impl From<&ConversationEffect> for ConversationUiEffect {
    fn from(effect: &ConversationEffect) -> Self {
        Self {
            copy: effect.copy.clone(),
            edit_draft: effect.edit_draft.clone(),
        }
    }
}
